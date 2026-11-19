namespace doublyLinkedList
{
    internal class Program
    {
        static void Main(string[] args)
        {
            DoublyLinkedPersonList list = new DoublyLinkedPersonList();

            Person peter = new Person("Peter", "Falk", new DateTime(1927, 9, 16), Sex.Male);
            Person shera = new Person("Shera", "Danese", new DateTime(1949, 10, 9), Sex.Female);

            list.AddLast(peter);
            list.DebugPrint(); // "Peter Falk, 16.09.1927"

            list.AddLast(shera);
            list.DebugPrint(); // "Peter Falk, 16.09.1927 <--> Shera Danese, 09.10.1949"

            Person leonardo = new Person("Leonardo", "DiCaprio", new DateTime(1974, 11, 11), Sex.Male);

            list.AddAfter(peter, leonardo);
            list.DebugPrint(); // "Peter Falk, 16.09.1927 <--> Leonardo DiCaprio, 11.11.1974 <--> Shera Danese, 09.10.1949"

            list.Clear();
            list.DebugPrint(); // ""

            list.AddLast(shera);
            list.DebugPrint(); // "Shera Danese, 09.10.1949"

            list.AddFirst(peter);
            list.DebugPrint(); // "Peter Falk, 16.09.1927 <--> Shera Danese, 09.10.1949"

            list.AddAfter(shera, peter);
            list.DebugPrint(); // "Peter Falk, 16.09.1927 <--> Shera Danese, 09.10.1949 <--> Peter Falk, 16.09.1927"

            list.AddBefore(peter, leonardo);
            list.DebugPrint(); // "Leonardo DiCaprio, 11.11.1974 <--> Peter Falk, 16.09.1927 <--> Shera Danese, 09.10.1949 <--> Peter Falk, 16.09.1927"

            list.Remove(peter);
            list.DebugPrint(); // "Leonardo DiCaprio, 11.11.1974 <--> Shera Danese, 09.10.1949 <--> Peter Falk, 16.09.1927"

            list.AddFirst(peter);
            list.DebugPrint(); // "Peter Falk, 16.09.1927 <--> Leonardo DiCaprio, 11.11.1974 <--> Shera Danese, 09.10.1949 <--> Peter Falk, 16.09.1927"

            list.RemoveAll(peter);
            list.DebugPrint(); // "Leonardo DiCaprio, 11.11.1974 <--> Shera Danese, 09.10.1949"
        }
    }
}
